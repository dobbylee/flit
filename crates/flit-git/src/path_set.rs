use std::collections::BTreeSet;

use crate::{GitChangeObservationError, GitRelativePath, MAX_GIT_STATUS_ENTRIES};

pub(crate) fn parse(output: &[u8]) -> Result<BTreeSet<GitRelativePath>, GitChangeObservationError> {
    if output.is_empty() {
        return Ok(BTreeSet::new());
    }
    if !output.ends_with(&[0]) {
        return Err(GitChangeObservationError::MalformedPathSet);
    }
    let mut paths = BTreeSet::new();
    for raw in output[..output.len() - 1].split(|byte| *byte == 0) {
        let path = GitRelativePath::new(raw.to_vec())?;
        if !paths.insert(path) {
            return Err(GitChangeObservationError::DuplicatePathSetRecord);
        }
        if paths.len() > MAX_GIT_STATUS_ENTRIES {
            return Err(GitChangeObservationError::TooManyPathSetEntries);
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use crate::{GitChangeObservationError, MAX_GIT_STATUS_ENTRIES};

    #[test]
    fn parses_raw_bounded_path_sets() {
        let paths = parse(b"a\0directory/b\0non-utf8-\xff\0").expect("paths");
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn rejects_malformed_duplicate_and_unbounded_sets() {
        assert_eq!(
            parse(b"path").expect_err("unterminated"),
            GitChangeObservationError::MalformedPathSet
        );
        assert_eq!(
            parse(b"same\0same\0").expect_err("duplicate"),
            GitChangeObservationError::DuplicatePathSetRecord
        );
        let mut too_many = Vec::new();
        for index in 0..=MAX_GIT_STATUS_ENTRIES {
            too_many.extend_from_slice(format!("path-{index}\0").as_bytes());
        }
        assert_eq!(
            parse(&too_many).expect_err("too many"),
            GitChangeObservationError::TooManyPathSetEntries
        );
    }
}
