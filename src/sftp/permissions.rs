#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum RemoteFileType {
    File,
    Directory,
    Symlink,
    Other,
    Unknown,
}

pub fn file_type_from_mode(mode: Option<u32>) -> RemoteFileType {
    match mode.map(|mode| mode & 0o170_000) {
        Some(0o040_000) => RemoteFileType::Directory,
        Some(0o100_000) => RemoteFileType::File,
        Some(0o120_000) => RemoteFileType::Symlink,
        Some(_) => RemoteFileType::Other,
        None => RemoteFileType::Unknown,
    }
}

pub fn format_permissions(mode: Option<u32>) -> String {
    let Some(mode) = mode else {
        return "--".to_string();
    };

    let file_type = match mode & 0o170_000 {
        0o010_000 => 'p',
        0o020_000 => 'c',
        0o040_000 => 'd',
        0o060_000 => 'b',
        0o100_000 => '-',
        0o120_000 => 'l',
        0o140_000 => 's',
        _ => '?',
    };

    let mut symbolic = String::with_capacity(10);
    symbolic.push(file_type);
    symbolic.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    symbolic.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    symbolic.push(match (mode & 0o100 != 0, mode & 0o4_000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });
    symbolic.push(if mode & 0o040 != 0 { 'r' } else { '-' });
    symbolic.push(if mode & 0o020 != 0 { 'w' } else { '-' });
    symbolic.push(match (mode & 0o010 != 0, mode & 0o2_000 != 0) {
        (true, true) => 's',
        (false, true) => 'S',
        (true, false) => 'x',
        (false, false) => '-',
    });
    symbolic.push(if mode & 0o004 != 0 { 'r' } else { '-' });
    symbolic.push(if mode & 0o002 != 0 { 'w' } else { '-' });
    symbolic.push(match (mode & 0o001 != 0, mode & 0o1_000 != 0) {
        (true, true) => 't',
        (false, true) => 'T',
        (true, false) => 'x',
        (false, false) => '-',
    });

    format!("{symbolic} {:04o}", mode & 0o7_777)
}

#[cfg(test)]
mod tests {
    use super::{RemoteFileType, file_type_from_mode, format_permissions};

    #[test]
    fn classifies_remote_file_types() {
        assert_eq!(file_type_from_mode(Some(0o100644)), RemoteFileType::File);
        assert_eq!(
            file_type_from_mode(Some(0o040755)),
            RemoteFileType::Directory
        );
        assert_eq!(file_type_from_mode(Some(0o120777)), RemoteFileType::Symlink);
        assert_eq!(file_type_from_mode(None), RemoteFileType::Unknown);
    }

    #[test]
    fn formats_regular_file_permissions() {
        assert_eq!(format_permissions(Some(0o100644)), "-rw-r--r-- 0644");
    }

    #[test]
    fn formats_directory_permissions() {
        assert_eq!(format_permissions(Some(0o040755)), "drwxr-xr-x 0755");
    }

    #[test]
    fn formats_symlink_and_special_permission_bits() {
        assert_eq!(format_permissions(Some(0o120777)), "lrwxrwxrwx 0777");
        assert_eq!(format_permissions(Some(0o104755)), "-rwsr-xr-x 4755");
        assert_eq!(format_permissions(Some(0o102644)), "-rw-r-Sr-- 2644");
        assert_eq!(format_permissions(Some(0o041777)), "drwxrwxrwt 1777");
    }

    #[test]
    fn formats_unknown_permissions() {
        assert_eq!(format_permissions(None), "--");
    }
}
