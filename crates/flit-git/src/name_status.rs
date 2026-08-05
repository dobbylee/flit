use std::collections::BTreeMap;

use crate::{GitChangeObservationError, GitFileStatus, GitRelativePath, MAX_GIT_STATUS_ENTRIES};

pub(crate) fn parse(
    output: &[u8],
) -> Result<BTreeMap<GitRelativePath, GitFileStatus>, GitChangeObservationError> {
    if output.is_empty() {
        return Ok(BTreeMap::new());
    }
    if !output.ends_with(&[0]) {
        return Err(GitChangeObservationError::MalformedNameStatus);
    }
    let fields = output[..output.len() - 1]
        .split(|byte| *byte == 0)
        .collect::<Vec<_>>();
    if fields.len() % 2 != 0 {
        return Err(GitChangeObservationError::MalformedNameStatus);
    }
    let mut records = BTreeMap::new();
    for pair in fields.chunks_exact(2) {
        let status = match pair[0] {
            b"A" => GitFileStatus::Added,
            b"M" => GitFileStatus::Modified,
            b"D" => GitFileStatus::Deleted,
            b"T" => GitFileStatus::TypeChanged,
            _ => return Err(GitChangeObservationError::MalformedNameStatus),
        };
        let path = GitRelativePath::new(pair[1].to_vec())?;
        if records.insert(path, status).is_some() {
            return Err(GitChangeObservationError::DuplicateNameStatusRecord);
        }
        if records.len() > MAX_GIT_STATUS_ENTRIES {
            return Err(GitChangeObservationError::TooManyNameStatusEntries);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::{GitChangeObservationError, GitFileStatus, MAX_GIT_STATUS_ENTRIES};

    #[test]
    fn parses_exact_non_rename_status_records() {
        let records = parse(b"A\0added\0M\0modified\0D\0deleted\0T\0typed\0").expect("name status");
        assert_eq!(records.len(), 4);
        assert_eq!(
            records.values().copied().collect::<Vec<_>>(),
            vec![
                GitFileStatus::Added,
                GitFileStatus::Deleted,
                GitFileStatus::Modified,
                GitFileStatus::TypeChanged,
            ]
        );
    }

    #[test]
    fn rejects_rename_malformed_duplicate_and_unbounded_records() {
        for malformed in [
            b"M\0path".as_slice(),
            b"M\0",
            b"R100\0old\0new\0",
            b"X\0path\0",
            b"M\0path\0dangling\0",
        ] {
            assert!(parse(malformed).is_err());
        }
        assert_eq!(
            parse(b"M\0same\0A\0same\0").expect_err("duplicate"),
            GitChangeObservationError::DuplicateNameStatusRecord
        );
        let mut too_many = Vec::new();
        for index in 0..=MAX_GIT_STATUS_ENTRIES {
            too_many.extend_from_slice(b"M\0");
            too_many.extend_from_slice(format!("path-{index}\0").as_bytes());
        }
        assert_eq!(
            parse(&too_many).expect_err("too many"),
            GitChangeObservationError::TooManyNameStatusEntries
        );
    }
}
