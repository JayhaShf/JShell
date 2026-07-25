use anyhow::{Result, anyhow};

pub const PAGE_BYTES: usize = 512 * 1024;
pub const PAGE_READ_BYTES: usize = PAGE_BYTES + 8 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LargeFileState {
    pub offset: u64,
    pub start_offset: u64,
    pub end_offset: u64,
    pub text: String,
    pub loading: bool,
    pub operation_id: String,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormalizedWindow {
    pub text: String,
    pub start_offset: u64,
    pub end_offset: u64,
}

pub fn count_matches(text: &str, query: &str) -> usize {
    if query.is_empty() {
        0
    } else {
        text.match_indices(query).count()
    }
}

pub fn normalize_window(bytes: &[u8], requested_offset: u64) -> Result<NormalizedWindow> {
    let mut start = bytes
        .iter()
        .position(|byte| byte & 0b1100_0000 != 0b1000_0000)
        .unwrap_or(bytes.len());
    if requested_offset > 0 {
        start = bytes[start..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|newline| start + newline + 1)
            .unwrap_or(bytes.len());
    }

    let body = &bytes[start..];
    let end = match std::str::from_utf8(body) {
        Ok(_) => bytes.len(),
        Err(error) if error.error_len().is_none() => start + error.valid_up_to(),
        Err(error) => {
            return Err(anyhow!(
                "invalid UTF-8 at remote byte {}",
                requested_offset + start as u64 + error.valid_up_to() as u64
            ));
        }
    };
    let text = std::str::from_utf8(&bytes[start..end])?.to_string();

    Ok(NormalizedWindow {
        text,
        start_offset: requested_offset + start as u64,
        end_offset: requested_offset + end as u64,
    })
}

#[cfg(test)]
mod tests {
    use super::{count_matches, normalize_window};

    #[test]
    fn counts_matches_in_the_current_chunk() {
        assert_eq!(count_matches("alpha beta alpha", "alpha"), 2);
        assert_eq!(count_matches("Alpha", "alpha"), 0);
        assert_eq!(count_matches("alpha", ""), 0);
    }

    #[test]
    fn skips_an_incomplete_leading_character_and_partial_line() {
        let bytes = "界\nalpha\n".as_bytes();
        let window = normalize_window(&bytes[1..], 1).expect("window should be valid UTF-8");

        assert_eq!(window.text, "alpha\n");
        assert_eq!(window.start_offset, 4);
    }

    #[test]
    fn nonzero_window_starts_after_the_first_complete_newline() {
        let window =
            normalize_window(b"partial\nnext\n", 100).expect("window should be valid UTF-8");

        assert_eq!(window.text, "next\n");
        assert_eq!(window.start_offset, 108);
    }

    #[test]
    fn truncates_an_incomplete_trailing_character() {
        let complete = "alpha\n界".as_bytes();
        let window = normalize_window(&complete[..complete.len() - 1], 0)
            .expect("partial trailing characters are page boundaries");

        assert_eq!(window.text, "alpha\n");
        assert_eq!(window.end_offset, 6);
    }

    #[test]
    fn rejects_invalid_utf8_inside_the_window() {
        assert!(normalize_window(b"alpha\n\xffomega", 0).is_err());
    }
}
