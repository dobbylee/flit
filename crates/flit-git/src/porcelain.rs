use std::collections::HashSet;

use crate::{
    DirtySummary, GitHead, GitObservationError, MAX_GIT_PATH_BYTES, MAX_GIT_STATUS_ENTRIES,
};

pub(crate) fn parse_status(output: &[u8]) -> Result<(GitHead, DirtySummary), GitObservationError> {
    if !output.ends_with(&[0]) {
        return Err(GitObservationError::MalformedPorcelain);
    }
    let records = output.split(|byte| *byte == 0).collect::<Vec<_>>();
    let mut index = 0_usize;
    let mut head = None;
    let mut branch_head_seen = false;
    let mut upstream_seen = false;
    let mut ahead_behind_seen = false;
    let mut entries_started = false;
    let mut entry_count = 0_u32;
    let mut staged = 0_u32;
    let mut unstaged = 0_u32;
    let mut untracked = 0_u32;
    let mut paths = HashSet::<Vec<u8>>::new();

    while index + 1 < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            return Err(GitObservationError::MalformedPorcelain);
        }
        if let Some(header) = record.strip_prefix(b"# ") {
            if entries_started {
                return Err(GitObservationError::MalformedPorcelain);
            }
            if let Some(value) = header.strip_prefix(b"branch.oid ") {
                if head.is_some() {
                    return Err(GitObservationError::DuplicatePorcelainRecord);
                }
                head = Some(parse_head(value)?);
            } else if let Some(value) = header.strip_prefix(b"branch.head ") {
                if branch_head_seen {
                    return Err(GitObservationError::DuplicatePorcelainRecord);
                }
                branch_head_seen = true;
                if value.is_empty() {
                    return Err(GitObservationError::MalformedPorcelain);
                }
            } else if let Some(value) = header.strip_prefix(b"branch.upstream ") {
                if upstream_seen {
                    return Err(GitObservationError::DuplicatePorcelainRecord);
                }
                upstream_seen = true;
                if value.is_empty() {
                    return Err(GitObservationError::MalformedPorcelain);
                }
            } else if let Some(value) = header.strip_prefix(b"branch.ab ") {
                if ahead_behind_seen {
                    return Err(GitObservationError::DuplicatePorcelainRecord);
                }
                ahead_behind_seen = true;
                if !valid_ahead_behind(value) {
                    return Err(GitObservationError::MalformedPorcelain);
                }
            } else {
                return Err(GitObservationError::MalformedPorcelain);
            }
            continue;
        }

        entries_started = true;
        entry_count = entry_count
            .checked_add(1)
            .ok_or(GitObservationError::TooManyPorcelainEntries)?;
        if entry_count as usize > MAX_GIT_STATUS_ENTRIES {
            return Err(GitObservationError::TooManyPorcelainEntries);
        }

        if record.starts_with(b"1 ") {
            let fields = split_exact(record, 9)?;
            validate_xy(fields[1], false)?;
            validate_submodule(fields[2])?;
            validate_modes_and_hashes(&fields[3..8], 3)?;
            record_path(fields[8], &mut paths)?;
            count_xy(fields[1], &mut staged, &mut unstaged);
        } else if record.starts_with(b"2 ") {
            let fields = split_exact(record, 10)?;
            validate_xy(fields[1], true)?;
            validate_submodule(fields[2])?;
            validate_modes_and_hashes(&fields[3..8], 3)?;
            validate_score(fields[8])?;
            record_path(fields[9], &mut paths)?;
            let source_path = records
                .get(index)
                .copied()
                .ok_or(GitObservationError::MalformedPorcelain)?;
            index += 1;
            validate_path(source_path)?;
            count_xy(fields[1], &mut staged, &mut unstaged);
        } else if record.starts_with(b"u ") {
            let fields = split_exact(record, 11)?;
            validate_unmerged_xy(fields[1])?;
            validate_submodule(fields[2])?;
            validate_modes_and_hashes(&fields[3..10], 4)?;
            record_path(fields[10], &mut paths)?;
            staged = staged.saturating_add(1);
            unstaged = unstaged.saturating_add(1);
        } else if let Some(path) = record.strip_prefix(b"? ") {
            record_path(path, &mut paths)?;
            untracked = untracked.saturating_add(1);
        } else {
            return Err(GitObservationError::MalformedPorcelain);
        }
    }

    let head = head.ok_or(GitObservationError::MalformedPorcelain)?;
    if !branch_head_seen {
        return Err(GitObservationError::MalformedPorcelain);
    }
    Ok((
        head,
        DirtySummary {
            staged,
            unstaged,
            untracked,
            entries: entry_count,
        },
    ))
}

fn parse_head(value: &[u8]) -> Result<GitHead, GitObservationError> {
    if value == b"(initial)" {
        return Ok(GitHead::Unborn);
    }
    if !valid_object_id(value) {
        return Err(GitObservationError::MalformedPorcelain);
    }
    Ok(GitHead::Available(
        std::str::from_utf8(value)
            .map_err(|_| GitObservationError::MalformedPorcelain)?
            .to_owned(),
    ))
}

fn split_exact(record: &[u8], fields: usize) -> Result<Vec<&[u8]>, GitObservationError> {
    let result = record
        .splitn(fields, |byte| *byte == b' ')
        .collect::<Vec<_>>();
    if result.len() != fields || result.iter().any(|field| field.is_empty()) {
        return Err(GitObservationError::MalformedPorcelain);
    }
    Ok(result)
}

fn validate_xy(value: &[u8], rename_record: bool) -> Result<(), GitObservationError> {
    if value.len() != 2 || !value.iter().all(|byte| b".MTADRCU".contains(byte)) {
        return Err(GitObservationError::MalformedPorcelain);
    }
    let has_rename = value.iter().any(|byte| matches!(byte, b'R' | b'C'));
    if has_rename != rename_record {
        return Err(GitObservationError::MalformedPorcelain);
    }
    Ok(())
}

fn validate_unmerged_xy(value: &[u8]) -> Result<(), GitObservationError> {
    if matches!(value, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU") {
        Ok(())
    } else {
        Err(GitObservationError::MalformedPorcelain)
    }
}

fn validate_submodule(value: &[u8]) -> Result<(), GitObservationError> {
    let valid = value == b"N..."
        || (value.len() == 4
            && value[0] == b'S'
            && matches!(value[1], b'.' | b'C')
            && matches!(value[2], b'.' | b'M')
            && matches!(value[3], b'.' | b'U'));
    if valid {
        Ok(())
    } else {
        Err(GitObservationError::MalformedPorcelain)
    }
}

fn validate_modes_and_hashes(
    values: &[&[u8]],
    mode_count: usize,
) -> Result<(), GitObservationError> {
    if values.len() <= mode_count
        || !values[..mode_count]
            .iter()
            .all(|value| value.len() == 6 && value.iter().all(|byte| matches!(byte, b'0'..=b'7')))
        || !values[mode_count..]
            .iter()
            .all(|value| valid_object_id(value))
    {
        return Err(GitObservationError::MalformedPorcelain);
    }
    Ok(())
}

fn validate_score(value: &[u8]) -> Result<(), GitObservationError> {
    let Some((kind, digits)) = value.split_first() else {
        return Err(GitObservationError::MalformedPorcelain);
    };
    if !matches!(kind, b'R' | b'C')
        || digits.is_empty()
        || digits.len() > 3
        || !digits.iter().all(u8::is_ascii_digit)
        || std::str::from_utf8(digits)
            .ok()
            .and_then(|digits| digits.parse::<u16>().ok())
            .is_none_or(|score| score > 100)
    {
        return Err(GitObservationError::MalformedPorcelain);
    }
    Ok(())
}

fn record_path(path: &[u8], paths: &mut HashSet<Vec<u8>>) -> Result<(), GitObservationError> {
    validate_path(path)?;
    if !paths.insert(path.to_vec()) {
        return Err(GitObservationError::DuplicatePorcelainRecord);
    }
    Ok(())
}

fn validate_path(path: &[u8]) -> Result<(), GitObservationError> {
    if path.is_empty() {
        return Err(GitObservationError::MalformedPorcelain);
    }
    if path.len() > MAX_GIT_PATH_BYTES {
        return Err(GitObservationError::GitPathTooLong);
    }
    Ok(())
}

fn count_xy(value: &[u8], staged: &mut u32, unstaged: &mut u32) {
    if value[0] != b'.' {
        *staged = staged.saturating_add(1);
    }
    if value[1] != b'.' {
        *unstaged = unstaged.saturating_add(1);
    }
}

fn valid_object_id(value: &[u8]) -> bool {
    matches!(value.len(), 40 | 64)
        && value
            .iter()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn valid_ahead_behind(value: &[u8]) -> bool {
    let Some(separator) = value.iter().position(|byte| *byte == b' ') else {
        return false;
    };
    let ahead = &value[..separator];
    let behind = &value[separator + 1..];
    valid_signed_count(ahead, b'+') && valid_signed_count(behind, b'-')
}

fn valid_signed_count(value: &[u8], sign: u8) -> bool {
    value.len() >= 2 && value[0] == sign && value[1..].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::parse_status;
    use crate::{GitHead, GitObservationError, MAX_GIT_PATH_BYTES, MAX_GIT_STATUS_ENTRIES};

    const OID: &str = "0123456789abcdef0123456789abcdef01234567";

    #[test]
    fn parses_content_free_categories_and_rename_source() {
        let mut output = format!(
            "# branch.oid {OID}\0# branch.head main\01 M. N... 100644 100644 100644 {OID} {OID} staged name\01 .M N... 100644 100644 100644 {OID} {OID} unstaged\02 R. N... 100644 100644 100644 {OID} {OID} R100 renamed to\0renamed from\0? non-utf8-"
        )
        .into_bytes();
        output.extend_from_slice(&[0xff, 0]);
        let (head, dirty) = parse_status(&output).expect("valid status");

        assert_eq!(head, GitHead::Available(OID.to_owned()));
        assert_eq!(dirty.staged, 2);
        assert_eq!(dirty.unstaged, 1);
        assert_eq!(dirty.untracked, 1);
        assert_eq!(dirty.entries, 4);
    }

    #[test]
    fn distinguishes_unborn_and_rejects_duplicate_or_malformed_records() {
        assert_eq!(
            parse_status(b"# branch.oid (initial)\0# branch.head main\0")
                .expect("unborn status")
                .0,
            GitHead::Unborn
        );
        let duplicate = format!("# branch.oid {OID}\0# branch.head main\0? same\0? same\0");
        assert_eq!(
            parse_status(duplicate.as_bytes()).expect_err("duplicate path"),
            GitObservationError::DuplicatePorcelainRecord
        );
        assert_eq!(
            parse_status(format!("# branch.oid {OID}\0# branch.head main").as_bytes())
                .expect_err("missing NUL"),
            GitObservationError::MalformedPorcelain
        );
    }

    #[test]
    fn rename_source_can_reappear_as_a_primary_untracked_entry() {
        let output = format!(
            "# branch.oid {OID}\0# branch.head main\02 R. N... 100644 100644 100644 {OID} {OID} R100 new\0old\0? old\0"
        );
        let (_, dirty) = parse_status(output.as_bytes()).expect("move and recreate status");

        assert_eq!(dirty.staged, 1);
        assert_eq!(dirty.unstaged, 0);
        assert_eq!(dirty.untracked, 1);
        assert_eq!(dirty.entries, 2);
    }

    #[test]
    fn enforces_entry_and_path_bounds() {
        let mut too_many = format!("# branch.oid {OID}\0# branch.head main\0").into_bytes();
        for index in 0..=MAX_GIT_STATUS_ENTRIES {
            too_many.extend_from_slice(format!("? path-{index}\0").as_bytes());
        }
        assert_eq!(
            parse_status(&too_many).expect_err("entry bound"),
            GitObservationError::TooManyPorcelainEntries
        );

        let mut long_path = format!("# branch.oid {OID}\0# branch.head main\0? ").into_bytes();
        long_path.extend(std::iter::repeat_n(b'x', MAX_GIT_PATH_BYTES + 1));
        long_path.push(0);
        assert_eq!(
            parse_status(&long_path).expect_err("path bound"),
            GitObservationError::GitPathTooLong
        );
    }
}
