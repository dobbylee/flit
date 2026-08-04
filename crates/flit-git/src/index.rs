use std::collections::HashSet;

use crate::{
    GitChangeObservationError, GitObservationError, MAX_GIT_PATH_BYTES, MAX_GIT_STATUS_ENTRIES,
    porcelain::valid_object_id,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct IndexSummary {
    pub contains_submodules: bool,
}

pub(crate) fn parse(output: &[u8]) -> Result<IndexSummary, GitChangeObservationError> {
    if output.is_empty() {
        return Ok(IndexSummary {
            contains_submodules: false,
        });
    }
    if !output.ends_with(&[0]) {
        return Err(GitChangeObservationError::MalformedIndex);
    }

    let mut count = 0_usize;
    let mut contains_submodules = false;
    let mut paths = HashSet::<Vec<u8>>::new();
    for record in output[..output.len() - 1].split(|byte| *byte == 0) {
        let Some(separator) = record.iter().position(|byte| *byte == b'\t') else {
            return Err(GitChangeObservationError::MalformedIndex);
        };
        let metadata = &record[..separator];
        let path = &record[separator + 1..];
        let fields = metadata.split(|byte| *byte == b' ').collect::<Vec<_>>();
        if fields.len() != 4
            || fields[0].len() != 1
            || !matches!(fields[1], b"100644" | b"100755" | b"120000" | b"160000")
            || !valid_object_id(fields[2])
            || fields[3].len() != 1
            || !matches!(fields[3][0], b'0'..=b'3')
            || path.is_empty()
        {
            return Err(GitChangeObservationError::MalformedIndex);
        }
        if fields[0] != b"H" {
            return Err(GitChangeObservationError::IndexFlagsUnsupportedForChanges);
        }
        if fields[3] != b"0" {
            return Err(GitChangeObservationError::UnmergedIndex);
        }
        if path.len() > MAX_GIT_PATH_BYTES {
            return Err(GitObservationError::GitPathTooLong.into());
        }
        if !paths.insert(path.to_vec()) {
            return Err(GitChangeObservationError::DuplicateIndexRecord);
        }
        count = count
            .checked_add(1)
            .ok_or(GitChangeObservationError::TooManyIndexEntries)?;
        if count > MAX_GIT_STATUS_ENTRIES {
            return Err(GitChangeObservationError::TooManyIndexEntries);
        }
        contains_submodules |= fields[1] == b"160000";
    }

    Ok(IndexSummary {
        contains_submodules,
    })
}

#[cfg(test)]
mod tests {
    use super::{IndexSummary, parse};
    use crate::{
        GitChangeObservationError, GitObservationError, MAX_GIT_PATH_BYTES, MAX_GIT_STATUS_ENTRIES,
    };

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn detects_gitlinks_without_retaining_paths() {
        let output =
            format!("H 100644 {OID} 0\ttracked\0H \x31\x36\x30\x30\x30\x30 {OID} 0\tmodule\0");
        assert_eq!(
            parse(output.as_bytes()).expect("bounded index"),
            IndexSummary {
                contains_submodules: true,
            }
        );
        assert_eq!(
            parse(b"").expect("empty index"),
            IndexSummary {
                contains_submodules: false,
            }
        );
    }

    #[test]
    fn rejects_malformed_duplicate_unmerged_and_unbounded_index_records() {
        for malformed in [
            format!("H 100644 {OID} 0 tracked\0"),
            format!("H 100600 {OID} 0\ttracked\0"),
            "H 100644 bad 0\ttracked\0".to_owned(),
            format!("H 100644 {OID} 4\ttracked\0"),
            format!("H 100644 {OID} 0\t\0"),
            format!("H 100644 {OID} 0\ttracked"),
        ] {
            assert_eq!(
                parse(malformed.as_bytes()).expect_err("malformed index"),
                GitChangeObservationError::MalformedIndex
            );
        }
        assert_eq!(
            parse(format!("H 100644 {OID} 1\ttracked\0").as_bytes()).expect_err("unmerged index"),
            GitChangeObservationError::UnmergedIndex
        );
        assert_eq!(
            parse(
                format!("H 100644 {OID} 0\tsame\0H \x31\x30\x30\x36\x34\x34 {OID} 0\tsame\0")
                    .as_bytes()
            )
            .expect_err("duplicate index path"),
            GitChangeObservationError::DuplicateIndexRecord
        );

        let mut long_path = format!("H 100644 {OID} 0\t").into_bytes();
        long_path.extend(std::iter::repeat_n(b'x', MAX_GIT_PATH_BYTES + 1));
        long_path.push(0);
        assert_eq!(
            parse(&long_path).expect_err("long index path"),
            GitChangeObservationError::Observation(GitObservationError::GitPathTooLong)
        );

        let mut too_many = Vec::new();
        for index in 0..=MAX_GIT_STATUS_ENTRIES {
            too_many.extend_from_slice(format!("H 100644 {OID} 0\tpath-{index}\0").as_bytes());
        }
        assert_eq!(
            parse(&too_many).expect_err("index entry bound"),
            GitChangeObservationError::TooManyIndexEntries
        );
    }

    #[test]
    fn rejects_assume_unchanged_and_skip_worktree_flags() {
        for flagged in [
            format!("h 100644 {OID} 0\ttracked\0"),
            format!("S 100644 {OID} 0\ttracked\0"),
            format!("s 100644 {OID} 0\ttracked\0"),
        ] {
            assert_eq!(
                parse(flagged.as_bytes()).expect_err("unsupported index flag"),
                GitChangeObservationError::IndexFlagsUnsupportedForChanges
            );
        }
    }
}
