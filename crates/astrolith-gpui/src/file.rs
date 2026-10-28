//! Bounded readonly source loading. This module never writes a document.
use std::{
    fs::File,
    io::{self, Read},
    path::PathBuf,
};

pub(crate) struct DocumentSource {
    pub path: PathBuf,
    pub source: String,
}

pub(crate) fn read_document(path: PathBuf) -> io::Result<DocumentSource> {
    let file = File::open(&path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::Error::other("Choose a regular file."));
    }
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(io::Error::other("Readonly files are limited to 8 MiB."));
    }
    let source =
        String::from_utf8(bytes).map_err(|_| io::Error::other("Choose a UTF-8 text file."))?;
    if source.lines().count() > 50000 {
        return Err(io::Error::other(
            "Readonly files are limited to 50,000 lines.",
        ));
    }
    Ok(DocumentSource { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_and_mdx_snapshots_preserve_source_bytes() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["中文.md", "page.mdx"] {
            let path = directory.path().join(name);
            let source =
                "---\r\ntitle: 日本語\r\n---\r\nimport X from './X';\r\n<X>{1 + 1}</X> 🦀\r\n";
            std::fs::write(&path, source).unwrap();
            let snapshot = read_document(path.clone()).unwrap();
            assert_eq!(snapshot.source, source);
            assert_eq!(snapshot.path, path);
            assert_eq!(std::fs::read_to_string(&snapshot.path).unwrap(), source);
        }
    }

    #[test]
    fn invalid_utf8_missing_nonregular_and_oversized_sources_fail_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        assert!(read_document(directory.path().join("missing.md")).is_err());
        assert!(read_document(directory.path().to_path_buf()).is_err());
        let path = directory.path().join("invalid.md");
        std::fs::write(&path, [0xff, 0xfe]).unwrap();
        assert!(read_document(path.clone()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xff, 0xfe]);
        let path = directory.path().join("large.md");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(8 * 1024 * 1024 + 1)
            .unwrap();
        assert!(read_document(path).is_err());
    }
}
