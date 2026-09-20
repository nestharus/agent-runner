//! A live traversal records observations, not a point-in-time resource inventory.
use std::{
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Debug, Default)]
pub(super) struct Census {
    pub observed_files: u64,
    pub observed_bytes: u64,
    pub disappeared: Vec<PathBuf>,
}

pub(super) fn observe(root: &Path) -> io::Result<Census> {
    let mut census = Census::default();
    visit(root, &mut census, &mut |_| {})?;
    Ok(census)
}

fn visit(
    path: &Path,
    census: &mut Census,
    before_metadata: &mut impl FnMut(&Path),
) -> io::Result<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            census.disappeared.push(path.into());
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    for entry in entries {
        // Enumeration errors without a path are not identifiable disappearances.
        let entry = entry?;
        let path = entry.path();
        before_metadata(&path);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                census.disappeared.push(path);
                continue;
            }
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            visit(&path, census, before_metadata)?;
        } else if metadata.is_file() {
            census.observed_files += 1;
            census.observed_bytes += metadata.len();
        }
    }
    Ok(())
}

#[test]
fn disappearance_is_explicit_not_a_zero_sized_file() {
    let dir = tempfile::tempdir().unwrap();
    let stable = dir.path().join("stable");
    let removed = dir.path().join("removed");
    fs::write(&stable, b"retained").unwrap();
    fs::write(&removed, b"not a zero byte file").unwrap();
    let mut census = Census::default();
    visit(dir.path(), &mut census, &mut |path| {
        if path == removed {
            fs::remove_file(path).unwrap();
        }
    })
    .unwrap();
    assert_eq!(census.observed_files, 1);
    assert_eq!(census.observed_bytes, 8);
    assert_eq!(census.disappeared, vec![removed]);
}

#[test]
fn disappeared_directory_and_other_errors_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("absent");
    let census = observe(&absent).unwrap();
    assert_eq!(census.disappeared, vec![absent]);
    let file = dir.path().join("file");
    fs::write(&file, b"bytes").unwrap();
    assert_eq!(
        observe(&file).unwrap_err().kind(),
        io::ErrorKind::NotADirectory
    );
}
