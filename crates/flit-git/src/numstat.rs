use std::collections::BTreeMap;

#[cfg(test)]
use crate::GitChangeSummary;
use crate::{
    GitChangeObservationError, GitLineCounts, GitRelativePath, MAX_GIT_CHANGE_COUNT,
    MAX_GIT_STATUS_ENTRIES,
};

#[cfg(test)]
pub(crate) fn parse(output: &[u8]) -> Result<GitChangeSummary, GitChangeObservationError> {
    let records = parse_records(output)?;
    let mut summary = GitChangeSummary::default();
    for counts in records.values() {
        let counts = counts.ok_or(GitChangeObservationError::BinaryNumstat)?;
        summary.files = add_count(summary.files, 1)?;
        summary.insertions = add_count(summary.insertions, counts.insertions)?;
        summary.deletions = add_count(summary.deletions, counts.deletions)?;
    }
    Ok(summary)
}

pub(crate) fn parse_records(
    output: &[u8],
) -> Result<BTreeMap<GitRelativePath, Option<GitLineCounts>>, GitChangeObservationError> {
    if output.is_empty() {
        return Ok(BTreeMap::new());
    }
    if !output.ends_with(&[0]) {
        return Err(GitChangeObservationError::MalformedNumstat);
    }

    let mut records = BTreeMap::new();
    for record in output[..output.len() - 1].split(|byte| *byte == 0) {
        let fields = record.splitn(3, |byte| *byte == b'\t').collect::<Vec<_>>();
        if fields.len() != 3 || fields[2].is_empty() {
            return Err(GitChangeObservationError::MalformedNumstat);
        }
        let path = GitRelativePath::new(fields[2].to_vec())?;
        let counts = if fields[0] == b"-" && fields[1] == b"-" {
            None
        } else {
            Some(GitLineCounts {
                insertions: parse_count(fields[0])?,
                deletions: parse_count(fields[1])?,
            })
        };
        if records.insert(path, counts).is_some() {
            return Err(GitChangeObservationError::DuplicateNumstatRecord);
        }
        if records.len() > MAX_GIT_STATUS_ENTRIES {
            return Err(GitChangeObservationError::TooManyNumstatEntries);
        }
    }
    Ok(records)
}

fn parse_count(value: &[u8]) -> Result<u64, GitChangeObservationError> {
    if value.is_empty()
        || (value.len() > 1 && value[0] == b'0')
        || !value.iter().all(u8::is_ascii_digit)
    {
        return Err(GitChangeObservationError::MalformedNumstat);
    }
    let value = std::str::from_utf8(value)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(GitChangeObservationError::GitChangeCountTooLarge)?;
    if value > MAX_GIT_CHANGE_COUNT {
        return Err(GitChangeObservationError::GitChangeCountTooLarge);
    }
    Ok(value)
}

#[cfg(test)]
fn add_count(current: u64, increment: u64) -> Result<u64, GitChangeObservationError> {
    current
        .checked_add(increment)
        .filter(|value| *value <= MAX_GIT_CHANGE_COUNT)
        .ok_or(GitChangeObservationError::GitChangeCountTooLarge)
}

#[cfg(test)]
mod tests {
    use super::{parse, parse_records};
    use crate::{
        GitChangeObservationError, GitChangeSummary, GitObservationError, MAX_GIT_CHANGE_COUNT,
        MAX_GIT_PATH_BYTES, MAX_GIT_STATUS_ENTRIES,
    };

    #[test]
    fn parses_empty_and_content_free_aggregate_receipts() {
        assert_eq!(parse(b"").expect("empty diff"), GitChangeSummary::default());
        assert_eq!(
            parse(b"3\t2\tpath with tabs\tinside\0\x30\t4\tsecond\0").expect("numstat"),
            GitChangeSummary {
                files: 2,
                insertions: 3,
                deletions: 6,
            }
        );
    }

    #[test]
    fn rejects_binary_malformed_duplicate_and_unbounded_records() {
        let binary = parse_records(b"-\t-\tbinary\0").expect("binary record metadata");
        assert_eq!(binary.values().next(), Some(&None));
        assert_eq!(
            parse(b"-\t-\tbinary\0").expect_err("binary record"),
            GitChangeObservationError::BinaryNumstat
        );
        for malformed in [
            b"1\t2\tpath".as_slice(),
            b"1\t2\t\0",
            b"01\t2\tpath\0",
            b"1\t-\tpath\0",
            b"1\t2\tpath\0\0",
        ] {
            assert_eq!(
                parse(malformed).expect_err("malformed record"),
                GitChangeObservationError::MalformedNumstat
            );
        }
        assert_eq!(
            parse(b"1\t2\tsame\0\x31\t2\tsame\0").expect_err("duplicate path"),
            GitChangeObservationError::DuplicateNumstatRecord
        );

        let mut long_path = b"1\t2\t".to_vec();
        long_path.extend(std::iter::repeat_n(b'x', MAX_GIT_PATH_BYTES + 1));
        long_path.push(0);
        assert_eq!(
            parse(&long_path).expect_err("long path"),
            GitChangeObservationError::Observation(GitObservationError::GitPathTooLong)
        );

        let mut too_many = Vec::new();
        for index in 0..=MAX_GIT_STATUS_ENTRIES {
            too_many.extend_from_slice(format!("0\t0\tpath-{index}\0").as_bytes());
        }
        assert_eq!(
            parse(&too_many).expect_err("entry bound"),
            GitChangeObservationError::TooManyNumstatEntries
        );
    }

    #[test]
    fn rejects_individual_and_aggregate_count_overflow() {
        let over_bound = MAX_GIT_CHANGE_COUNT + 1;
        assert_eq!(
            parse(format!("{over_bound}\t0\tpath\0").as_bytes()).expect_err("count bound"),
            GitChangeObservationError::GitChangeCountTooLarge
        );
        assert_eq!(
            parse(format!("{MAX_GIT_CHANGE_COUNT}\t0\tfirst\01\t0\tsecond\0").as_bytes())
                .expect_err("aggregate bound"),
            GitChangeObservationError::GitChangeCountTooLarge
        );
    }
}
