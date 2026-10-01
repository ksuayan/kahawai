//! Deterministic, local metadata cleanup (docs/v1/kahawai-metadata-enrichment-spec.md,
//! "Remap"). Pure functions: display strings are never altered, these derive
//! the keys the catalog groups and sorts by.

/// Key for grouping names that differ only in case or spacing
/// ("Kind Of Blue" and "kind of  blue " are the same album). Lowercased,
/// runs of whitespace collapsed to one space, trimmed.
pub fn group_key(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Leading articles moved to the end for sorting.
const ARTICLES: &[&str] = &["The ", "A "];

/// Sort form of a name: a leading "The " or "A " moves to the end,
/// "The Beatles" -> "Beatles, The". Matched case-insensitively, original
/// casing kept. Names already in "Last, First" form, and a name that is
/// only the article, are left alone.
pub fn sort_key(s: &str) -> String {
    let t = s.trim();
    for article in ARTICLES {
        // `get` rather than slicing: byte n may fall inside a character.
        let (Some(head), Some(rest)) = (t.get(..article.len()), t.get(article.len()..)) else {
            continue;
        };
        let rest = rest.trim_start();
        if head.eq_ignore_ascii_case(article) && !rest.is_empty() {
            return format!("{rest}, {}", head.trim_end());
        }
    }
    t.to_string()
}

/// A tagged year, or `None` when it can't be a release year: before 1900 or
/// after next year (typos like 19999, or 0 from an empty field).
pub fn sane_year(year: Option<u16>, this_year: u16) -> Option<u16> {
    year.filter(|y| (1900..=this_year.saturating_add(1)).contains(y))
}

/// The current year (UTC), for [`sane_year`].
pub fn current_year() -> u16 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since 1970 to a civil year (Howard Hinnant's algorithm).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    u16::try_from(year).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_key_ignores_case_and_spacing_only() {
        assert_eq!(group_key("Kind Of Blue"), group_key("  kind of   blue "));
        assert_eq!(group_key("Kind of Blue"), "kind of blue");
        assert_ne!(group_key("Blue Train"), group_key("Blue Train (Remaster)"));
        assert_eq!(group_key("Björk"), "björk", "non-ASCII lowercases too");
    }

    #[test]
    fn sort_key_moves_a_leading_article_to_the_end() {
        assert_eq!(sort_key("The Beatles"), "Beatles, The");
        assert_eq!(sort_key("the national"), "national, the");
        assert_eq!(sort_key("A Tribe Called Quest"), "Tribe Called Quest, A");
        assert_eq!(sort_key("The Wall"), "Wall, The");
    }

    #[test]
    fn sort_key_leaves_everything_else_alone() {
        assert_eq!(
            sort_key("Beatles, The"),
            "Beatles, The",
            "already sorted form"
        );
        assert_eq!(sort_key("Bach, Johann Sebastian"), "Bach, Johann Sebastian");
        assert_eq!(
            sort_key("Theory of a Deadman"),
            "Theory of a Deadman",
            "The + word, no space"
        );
        assert_eq!(sort_key("Aphex Twin"), "Aphex Twin");
        assert_eq!(sort_key("The"), "The", "just the article");
        assert_eq!(sort_key("A"), "A");
        assert_eq!(sort_key("  Radiohead "), "Radiohead");
        assert_eq!(sort_key("Aé"), "Aé", "no panic on a multi-byte character");
        assert_eq!(sort_key("Été"), "Été");
    }

    #[test]
    fn sane_year_drops_impossible_years() {
        assert_eq!(sane_year(Some(1959), 2026), Some(1959));
        assert_eq!(sane_year(Some(1900), 2026), Some(1900));
        assert_eq!(
            sane_year(Some(2027), 2026),
            Some(2027),
            "next year: pre-release"
        );
        assert_eq!(sane_year(Some(2028), 2026), None);
        assert_eq!(sane_year(Some(1899), 2026), None);
        assert_eq!(sane_year(Some(0), 2026), None);
        assert_eq!(sane_year(None, 2026), None);
    }

    #[test]
    fn current_year_is_plausible() {
        let y = current_year();
        assert!((2024..2200).contains(&y), "{y}");
    }
}
