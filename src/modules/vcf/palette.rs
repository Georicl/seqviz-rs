//! Allele-aware VCF colours; GT and AD keep each ALT's original index.
use super::config::VcfConfig;
use crate::widgets::theme::Theme;
use ratatui::{
    style::{Color, Style},
    text::Span,
};

/// Use the foreground for reference allele 0 and cycle configured colours for ALT alleles.
///
/// # Arguments
///
/// - `index`: VCF allele index, with zero for REF and positive values for ALT alleles in source
///   order.
/// - `config`: VCF display and semantic-colour settings used to style the content.
/// - `theme`: Shared application colours for text, borders, focus and selection.
pub(crate) fn allele_color(index: usize, config: &VcfConfig, theme: &Theme) -> Color {
    if index == 0 || config.allele_colors.is_empty() {
        theme.foreground
    } else {
        config.allele_colors[(index - 1) % config.allele_colors.len()]
    }
}

/// Distinguish PASS, missing filters and named filtering failures.
///
/// # Arguments
///
/// - `value`: Original FILTER text, preserving PASS, missing `.` and named filters.
/// - `config`: VCF display and semantic-colour settings used to style the content.
pub(crate) fn filter_color(value: &str, config: &VcfConfig) -> Color {
    match value {
        "PASS" => config.pass_color,
        "." => config.missing_color,
        _ => config.filter_color,
    }
}

/// Keep complete numeric allele tokens together, including multi-digit alleles.
///
/// # Arguments
///
/// - `value`: Original GT text, including allele numbers, separators and missing values.
/// - `config`: VCF display and semantic-colour settings used to style the content.
/// - `theme`: Shared application colours for text, borders, focus and selection.
pub(crate) fn genotype(value: &str, config: &VcfConfig, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut kind = 0;
    for (at, ch) in value.char_indices() {
        let next = if ch.is_ascii_digit() {
            1
        } else if ch == '.' {
            2
        } else {
            0
        };
        if at != start && next != kind {
            spans.push(genotype_token(&value[start..at], kind == 1, config, theme));
            start = at;
        }
        kind = next;
    }
    if start < value.len() {
        spans.push(genotype_token(&value[start..], kind == 1, config, theme));
    }
    spans
}

/// Style a whole allele number, missing marker or genotype separator.
///
/// # Arguments
///
/// - `value`: One genotype allele, missing-value marker or separator token.
/// - `digits`: Whether the genotype token contains a numeric allele index.
/// - `config`: VCF display and semantic-colour settings used to style the content.
/// - `theme`: Shared application colours for text, borders, focus and selection.
fn genotype_token(value: &str, digits: bool, config: &VcfConfig, theme: &Theme) -> Span<'static> {
    let color = if digits {
        value
            .parse()
            .map(|index| allele_color(index, config, theme))
            .unwrap_or(theme.foreground)
    } else if value == "." {
        config.missing_color
    } else {
        theme.foreground
    };
    Span::styled(value.to_owned(), Style::default().fg(color))
}

/// Colour each displayed base using the configured nucleotide palette.
///
/// # Arguments
///
/// - `value`: Nucleotide allele text to colour one base at a time.
/// - `config`: VCF display and semantic-colour settings used to style the content.
pub(crate) fn nucleotides(value: &str, config: &VcfConfig) -> Vec<Span<'static>> {
    value
        .chars()
        .map(|base| {
            Span::styled(
                base.to_string(),
                Style::default().fg(config.nucleotides.nucleotide_color(base as u8)),
            )
        })
        .collect()
}

/// An AD segment uses the same reference/ALT index as its GT token.
///
/// # Arguments
///
/// - `value`: Comma-separated AD depths in REF/ALT allele order.
/// - `width`: Available width of the allele-depth bar in terminal columns.
/// - `config`: VCF display and semantic-colour settings used to style the content.
/// - `theme`: Shared application colours for text, borders, focus and selection.
pub(crate) fn allele_depth(
    value: &str,
    width: usize,
    config: &VcfConfig,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let Some(depths) = value
        .split(',')
        .map(|v| v.parse::<u64>().ok())
        .collect::<Option<Vec<_>>>()
    else {
        return Vec::new();
    };
    let total: u128 = depths.iter().map(|&n| u128::from(n)).sum();
    if total == 0 || width == 0 {
        return Vec::new();
    }
    let mut consumed = 0u128;
    let mut placed = 0usize;
    let mut spans = Vec::new();
    for (index, depth) in depths.into_iter().enumerate() {
        consumed += u128::from(depth);
        let next = (consumed * width as u128 / total) as usize;
        let count = next.saturating_sub(placed);
        if depth > 0 && count > 0 {
            spans.push(Span::styled(
                "█".repeat(count),
                Style::default().fg(allele_color(index, config, theme)),
            ));
        }
        placed = next;
    }
    spans
}

/// Check allele identity across genotype text and depth-bar segments.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify multi-digit genotypes and sparse AD values keep their original allele colours.
    #[test]
    fn complete_gt_tokens_and_ad_segments_share_allele_identity() {
        let config = VcfConfig::default();
        let theme = Theme::default();
        let spans = genotype("0|10/2/.", &config, &theme);
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<String>(),
            "0|10/2/."
        );
        let ten = spans.iter().find(|s| s.content == "10").unwrap();
        assert_eq!(ten.style.fg, Some(allele_color(10, &config, &theme)));
        assert_eq!(spans[0].style.fg, Some(theme.foreground));
        assert_eq!(
            spans.iter().find(|s| s.content == ".").unwrap().style.fg,
            Some(config.missing_color)
        );
        let missing = genotype("./.", &config, &theme);
        assert_eq!(missing[0].style.fg, Some(config.missing_color));
        assert_eq!(missing[1].style.fg, Some(theme.foreground));
        assert_eq!(missing[2].style.fg, Some(config.missing_color));
        let bars = allele_depth("0,4,0,6", 10, &config, &theme);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].content.len(), 4 * "█".len());
        assert_eq!(bars[0].style.fg, Some(allele_color(1, &config, &theme)));
        assert_eq!(bars[1].style.fg, Some(allele_color(3, &config, &theme)));
        assert!(allele_depth("0,0,0", 20, &config, &theme).is_empty());
    }
}
