use crate::{GitChangeObservationError, GitObservationError, MAX_GIT_PATH_BYTES};

pub(crate) fn validate(path: &[u8]) -> Result<(), GitChangeObservationError> {
    if path.is_empty()
        || path.len() > MAX_GIT_PATH_BYTES
        || path[0] == b'/'
        || path.contains(&0)
        || path
            .split(|byte| *byte == b'/')
            .any(|component| component.is_empty() || component == b"." || component == b"..")
    {
        return if path.len() > MAX_GIT_PATH_BYTES {
            Err(GitObservationError::GitPathTooLong.into())
        } else {
            Err(GitChangeObservationError::InvalidRelativePath)
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate;
    use crate::{
        GitChangeObservationError, GitObservationError, GitRelativePath, MAX_GIT_PATH_BYTES,
    };

    #[test]
    fn accepts_arbitrary_relative_path_bytes() {
        validate(b"directory/non-utf8-\xff").expect("bounded raw relative path");
        validate(b"tabs\tand\nnewlines").expect("control bytes are valid filename bytes");
        assert_eq!(
            format!(
                "{:?}",
                GitRelativePath::new(b"secret.txt".to_vec()).expect("path")
            ),
            "GitRelativePath(<redacted>)"
        );
    }

    #[test]
    fn rejects_ambiguous_or_unbounded_paths() {
        for path in [
            b"".as_slice(),
            b"/absolute",
            b".",
            b"..",
            b"a/./b",
            b"a/../b",
            b"a//b",
            b"a/",
            b"nul\0path",
        ] {
            assert_eq!(
                validate(path).expect_err("invalid relative path"),
                GitChangeObservationError::InvalidRelativePath
            );
        }
        assert_eq!(
            validate(&vec![b'x'; MAX_GIT_PATH_BYTES + 1]).expect_err("long path"),
            GitChangeObservationError::Observation(GitObservationError::GitPathTooLong)
        );
    }
}
