//! Recognise FASTQ naming conventions without modifying the original header.
//!
//! Sources: Illumina FASTQ File Format; ONT Output Specifications (FASTQ);
//! PacBioFileFormats QNAME convention; NCBI sra-tools fasterq-dump deflines.

/// Ordered parsed header properties, including a detected-format label.
type Properties = Vec<(String, String)>;

/// Extract present header fields and add a detected-format label. Unknown headers
/// produce no properties. Accepts headers with or without a leading `@`.
pub fn parse_fastq_header(header: &str) -> Properties {
    let header = header.trim_end_matches(['\r', '\n']);
    let header = header.strip_prefix('@').unwrap_or(header);
    let mut words = header.split_whitespace();
    let Some(name) = words.next() else {
        return Vec::new();
    };
    let rest: Vec<_> = words.collect();
    parse_sra(name, &rest)
        .or_else(|| parse_nanopore(name, &rest))
        .or_else(|| parse_pacbio(name))
        .or_else(|| parse_illumina(name, &rest))
        .unwrap_or_default()
}

/// Append a property only when its value is nonempty.
fn add(fields: &mut Properties, name: &str, value: &str) {
    if !value.is_empty() {
        fields.push((name.to_owned(), value.to_owned()));
    }
}

/// Recognise a nonempty sequence of ASCII decimal digits.
fn decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}
/// Recognise an ASCII decimal coordinate with an optional leading minus sign.
fn coordinate(value: &str) -> bool {
    decimal(value.strip_prefix('-').unwrap_or(value))
}

/// Recognise modern or legacy Illumina names and preserve only present read metadata.
fn parse_illumina(name: &str, rest: &[&str]) -> Option<Properties> {
    let parts: Vec<_> = name.split(':').collect();
    let mut fields = Vec::new();
    if (parts.len() == 7 || parts.len() == 8)
        && !parts[0].is_empty()
        && decimal(parts[1])
        && !parts[2].is_empty()
        && decimal(parts[3])
        && decimal(parts[4])
        && coordinate(parts[5])
        && coordinate(parts[6])
    {
        add(&mut fields, "Header format", "Illumina");
        for (label, value) in [
            "Instrument",
            "Run",
            "Flow cell",
            "Lane",
            "Tile",
            "X coordinate",
            "Y coordinate",
        ]
        .into_iter()
        .zip(&parts)
        {
            add(&mut fields, label, value);
        }
        if parts.len() == 8 {
            add(&mut fields, "UMI", parts[7]);
        }
        if let Some(read) = rest.first() {
            let read: Vec<_> = read.split(':').collect();
            if read.len() == 4
                && decimal(read[0])
                && matches!(read[1], "Y" | "N")
                && decimal(read[2])
            {
                add(&mut fields, "Read number", read[0]);
                add(&mut fields, "Filtered", read[1]);
                add(&mut fields, "Control number", read[2]);
                add(&mut fields, "Index", read[3]);
            }
        }
        return Some(fields);
    }
    // Legacy Illumina: instrument:lane:tile:x:y#index/read.
    if parts.len() == 5
        && !parts[0].is_empty()
        && decimal(parts[1])
        && decimal(parts[2])
        && coordinate(parts[3])
    {
        let (position, read) = parts[4]
            .rsplit_once('/')
            .map_or((parts[4], None), |(left, right)| (left, Some(right)));
        let (position, index) = position
            .split_once('#')
            .map_or((position, None), |(left, right)| (left, Some(right)));
        if !coordinate(position) || read.is_some_and(|r| !decimal(r)) {
            return None;
        }
        add(&mut fields, "Header format", "Illumina");
        for (label, value) in ["Instrument", "Lane", "Tile", "X coordinate"]
            .into_iter()
            .zip(&parts)
        {
            add(&mut fields, label, value);
        }
        add(&mut fields, "Y coordinate", position);
        if let Some(index) = index {
            add(&mut fields, "Index", index);
        }
        if let Some(read) = read {
            add(&mut fields, "Read number", read);
        }
        return Some(fields);
    }
    None
}

/// Recognise a UUID plus known ONT metadata and retain additional key/value attributes.
fn parse_nanopore(name: &str, rest: &[&str]) -> Option<Properties> {
    let groups: Vec<_> = name.split('-').collect();
    let is_uuid = groups.len() == 5
        && groups.iter().zip([8, 4, 4, 4, 12]).all(|(group, size)| {
            group.len() == size && group.bytes().all(|b| b.is_ascii_hexdigit())
        });
    if !is_uuid {
        return None;
    }
    let values: Vec<_> = rest
        .iter()
        .filter_map(|token| token.split_once('='))
        .filter(|(key, value)| !key.is_empty() && !value.is_empty())
        .collect();
    if !values.iter().any(|(key, _)| {
        matches!(
            *key,
            "runid"
                | "ch"
                | "start_time"
                | "flow_cell_id"
                | "protocol_group_id"
                | "basecall_model_version_id"
                | "basecall_model"
        )
    }) {
        return None;
    }
    let mut fields = vec![
        ("Header format".into(), "Nanopore".into()),
        ("Read ID".into(), name.into()),
    ];
    for (key, value) in values {
        let label = match key {
            "runid" => "Run ID",
            "read" => "Read number",
            "ch" => "Channel",
            "start_time" => "Start time",
            "flow_cell_id" => "Flow cell",
            "protocol_group_id" => "Protocol group ID",
            "sampleid" => "Sample ID",
            "barcode" => "Barcode",
            "barcode_alias" => "Barcode alias",
            "basecall_model_version_id" | "basecall_model" => "Basecall model",
            _ => key,
        };
        add(&mut fields, label, value);
    }
    Some(fields)
}

/// Extract movie, ZMW and optional subread or CCS segment coordinates from a QNAME.
fn parse_pacbio(name: &str) -> Option<Properties> {
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() < 3 || parts[0].is_empty() || !decimal(parts[1]) {
        return None;
    }
    let mut fields = vec![
        ("Header format".into(), "PacBio".into()),
        ("Movie".into(), parts[0].into()),
        ("ZMW hole".into(), parts[1].into()),
    ];
    let mut tail = &parts[2..];
    if tail[0] == "ccs" {
        add(&mut fields, "Read type", "ccs");
        tail = &tail[1..];
        if tail
            .first()
            .is_some_and(|part| matches!(*part, "fwd" | "rev"))
        {
            add(&mut fields, "CCS strand", tail[0]);
            tail = &tail[1..];
        }
        if tail.is_empty() {
            return Some(fields);
        }
    }
    if tail.len() != 1 {
        return None;
    }
    let (start, end) = tail[0].split_once('_')?;
    if !decimal(start) || !decimal(end) {
        return None;
    }
    add(&mut fields, "Query start (0-based)", start);
    add(&mut fields, "Query end (exclusive)", end);
    Some(fields)
}

/// Separate SRA run accession, spot and read identifiers, retaining supplied name and length.
fn parse_sra(name: &str, rest: &[&str]) -> Option<Properties> {
    let (id, slash_read) = name
        .split_once('/')
        .map_or((name, None), |(id, read)| (id, Some(read)));
    let parts: Vec<_> = id.split('.').collect();
    let accession = *parts.first()?;
    if accession.len() <= 3
        || !["SRR", "ERR", "DRR"]
            .iter()
            .any(|prefix| accession.starts_with(prefix))
        || !decimal(&accession[3..])
        || parts.len() > 3
        || parts[1..].iter().any(|part| !decimal(part))
        || slash_read.is_some_and(|read| !decimal(read))
    {
        return None;
    }
    let mut fields = vec![
        ("Header format".into(), "SRA".into()),
        ("Run accession".into(), accession.into()),
    ];
    if let Some(spot) = parts.get(1) {
        add(&mut fields, "Spot ID", spot);
    }
    if let Some(read) = slash_read.or_else(|| parts.get(2).copied()) {
        add(&mut fields, "Read number", read);
    }
    for (index, token) in rest.iter().enumerate() {
        if let Some(length) = token.strip_prefix("length=").filter(|value| decimal(value)) {
            add(&mut fields, "Header length", length);
        } else if index == 0 && !token.contains('=') {
            add(&mut fields, "Spot name", token);
        }
    }
    Some(fields)
}

/// Platform-header recognition and missing-field regressions.
#[cfg(test)]
mod tests {
    use super::*;

    /// Look up one derived header property without consuming the property list.
    fn value<'a>(fields: &'a Properties, key: &str) -> Option<&'a str> {
        fields
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// Check both Illumina conventions retain supplied coordinates, index and read metadata.
    #[test]
    fn illumina_modern_and_legacy_preserve_present_fields() {
        let fields = parse_fastq_header("@SIM:1:FCX:1:15:6329:1045:GATTACT+GTCTTAAC 1:N:0:ATCCGA");
        assert_eq!(value(&fields, "Header format"), Some("Illumina"));
        assert_eq!(value(&fields, "UMI"), Some("GATTACT+GTCTTAAC"));
        assert_eq!(value(&fields, "Filtered"), Some("N"));
        let fields = parse_fastq_header("HWUSI-EAS100R:6:73:941:1973#ATCG/1");
        assert_eq!(value(&fields, "Y coordinate"), Some("1973"));
        assert_eq!(value(&fields, "Index"), Some("ATCG"));
        assert_eq!(value(&fields, "Run"), None);
    }

    /// Check that a modern Illumina name does not manufacture absent UMI or read fields.
    #[test]
    fn absent_illumina_read_metadata_is_not_invented() {
        let fields = parse_fastq_header("SIM:1:FCX:1:15:-6329:1045");
        assert_eq!(fields.len(), 8);
        assert_eq!(value(&fields, "Read number"), None);
        assert_eq!(value(&fields, "UMI"), None);
    }

    /// Check ONT attributes retain their values, including unrecognised keys and embedded equals signs.
    #[test]
    fn nanopore_fields_keep_actual_values() {
        let fields = parse_fastq_header(
            "@4dc6bc7e-a96b-4ea9-9b31-cf6e12bf45e3 runid=run42 ch=87 start_time=2024-01-02T03:04:05Z sampleid=sample_A barcode=barcode01 custom=x=y",
        );
        assert_eq!(value(&fields, "Header format"), Some("Nanopore"));
        assert_eq!(value(&fields, "Channel"), Some("87"));
        assert_eq!(value(&fields, "custom"), Some("x=y"));
        assert_eq!(value(&fields, "Flow cell"), None);
    }

    /// Check PacBio subread, unsegmented CCS and strand-specific CCS coordinate forms.
    #[test]
    fn pacbio_subread_ccs_and_segmented_ccs() {
        let fields = parse_fastq_header("m64011_190830_220126/42/10_500");
        assert_eq!(value(&fields, "ZMW hole"), Some("42"));
        assert_eq!(value(&fields, "Query start (0-based)"), Some("10"));
        let fields = parse_fastq_header("m64011_190830_220126/42/ccs/rev/5_90");
        assert_eq!(value(&fields, "Read type"), Some("ccs"));
        assert_eq!(value(&fields, "CCS strand"), Some("rev"));
        assert_eq!(value(&fields, "Query end (exclusive)"), Some("90"));
        assert_eq!(
            value(
                &parse_fastq_header("m64011_190830_220126/42/ccs"),
                "Query start (0-based)"
            ),
            None
        );
        assert_eq!(
            value(&parse_fastq_header("movieName/42/ccs"), "Movie"),
            Some("movieName")
        );
    }

    /// Check SRA dot/slash suffixes retain distinct run, spot and read properties.
    #[test]
    fn sra_variants_separate_accession_spot_and_read() {
        let fields = parse_fastq_header("@SRR123456.78/2 original_name length=151");
        assert_eq!(value(&fields, "Run accession"), Some("SRR123456"));
        assert_eq!(value(&fields, "Spot ID"), Some("78"));
        assert_eq!(value(&fields, "Read number"), Some("2"));
        assert_eq!(value(&fields, "Spot name"), Some("original_name"));
        assert_eq!(value(&fields, "Header length"), Some("151"));
        assert_eq!(
            value(&parse_fastq_header("ERR12.5.1 length=80"), "Read number"),
            Some("1")
        );
        assert_eq!(value(&parse_fastq_header("DRR12.5"), "Header length"), None);
    }

    /// Check unknown names and malformed conventions yield no inferred platform properties.
    #[test]
    fn unknown_or_incomplete_patterns_produce_no_fabricated_properties() {
        for header in [
            "",
            "@custom-read comment",
            "x:a:b:c:d:e:f",
            "SRRexample.1",
            "m_movie/abc/ccs",
            "4dc6bc7e-a96b-4ea9-9b31-cf6e12bf45e3 arbitrary description",
        ] {
            assert!(parse_fastq_header(header).is_empty(), "{header}");
        }
    }
}
